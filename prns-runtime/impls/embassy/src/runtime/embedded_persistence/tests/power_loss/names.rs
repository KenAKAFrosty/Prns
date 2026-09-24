use super::*;

#[test]
fn uncertain_node_name_commit_is_resolved_before_a_queued_group_change() {
    embassy_futures::block_on(async {
        let groups = DiscoveryGroupConfigurationStoreExchange::new();
        let names = NodeNameStoreExchange::new();
        let control = Rc::new(RefCell::new(Control::new()));
        let policy =
            EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(0));
        let mut owner = EmbeddedFlashPersistence::<_, FixedRouteSnapshotKeys<8>, _, 4, _, _>::with_configuration_stores(
            Flash::boot([0xff; CAPACITY], control.clone()), LAYOUT, policy, FixedRouteSnapshotKeys::new(),
            (|_| {}) as fn(EmbeddedPersistenceDiagnostic), &groups, &names,
        );
        let mut engine = EngineState::<crate::storage::GrowableHeap>::default();
        let mut remote = available_remote_control(&mut engine);
        owner
            .restore(&mut engine, &mut remote, InstantMillis(0))
            .await;
        let name = RemoteControlNodeName::new("Name with lost commit acknowledgement").unwrap();
        let mut named = core::pin::pin!(names.store(name));
        control.borrow_mut().lose_commit_acknowledgement();
        owner.progress(&mut engine, WRITE_TIME).await;
        assert!(owner.pending_confirmation.is_some());
        assert_eq!(names.restored_now(), Some(None));
        let mut context = core::task::Context::from_waker(core::task::Waker::noop());
        assert!(core::future::Future::poll(named.as_mut(), &mut context).is_pending());

        let interface =
            crate::interfaces::InterfaceId::new([0x42; crate::interfaces::INTERFACE_ID_LEN]);
        let mut grouped = core::pin::pin!(groups.store(DiscoveryGroupConfigurationChange::upsert(
            interface,
            crate::interfaces::DiscoveryGroupSet::reticulum(),
        )));
        control.borrow_mut().arm(None);
        let retry = InstantMillis(WRITE_TIME.0 + policy.retry_interval_millis);
        owner.progress(&mut engine, retry).await;
        assert_eq!(
            core::future::Future::poll(named.as_mut(), &mut context),
            core::task::Poll::Ready(Ok(()))
        );
        assert_eq!(names.restored_now(), Some(Some(name)));
        assert!(groups.has_pending_request());
        assert!(control
            .borrow()
            .trace
            .iter()
            .all(|op| matches!(op, Operation::Read { .. })));

        for step in 1..=MAX_PROGRESS_STEPS {
            if !groups.has_pending_request() {
                break;
            }
            owner
                .progress(&mut engine, InstantMillis(retry.0 + step as u64))
                .await;
        }
        assert_eq!(
            core::future::Future::poll(grouped.as_mut(), &mut context),
            core::task::Poll::Ready(Ok(()))
        );
        assert_eq!(
            groups.groups_now(interface),
            Some(Some(crate::interfaces::DiscoveryGroupSet::reticulum()))
        );
        assert_eq!(names.restored_now(), Some(Some(name)));
    });
}
