use super::*;

fn announce(arrived_at_millis: u64) -> AuthenticatedAnnounce<'static> {
    AuthenticatedAnnounce {
        destination: prns_host::DestinationHash::new([1; 16]),
        announced_identity: prns_host::IdentityHash::new([2; 16]),
        source_interface: prns_host::InterfaceId::new([3; 8]),
        hops: 2,
        app_data: b"not retained or interpreted by activity",
        arrived_at_millis,
        is_path_response: true,
    }
}

fn record(store: &NetworkActivityStore, arrived: u64) {
    store.record(
        store
            .admit(announce(arrived))
            .expect("admitted observation"),
    );
}

#[test]
fn latest_200_admissions_are_bounded_and_newest_first_even_with_late_insertion() {
    let store = NetworkActivityStore::new(7);
    let initial_capacity = store.lock().rows.capacity();
    let delayed = store.admit(announce(1)).unwrap();
    for arrival in 2..=202 {
        record(&store, arrival);
    }
    let before_late = store.project(250);
    assert_eq!(before_late.rows.len(), 200);
    assert_eq!(before_late.rows.first().unwrap().record_id, 202);
    assert_eq!(before_late.rows.last().unwrap().record_id, 3);
    assert_eq!(before_late.evictions, 1);
    store.record(delayed);
    let after_late = store.project(250);
    assert_eq!(after_late.rows, before_late.rows);
    assert_eq!(after_late.evictions, 2);
    assert_eq!(store.lock().rows.capacity(), initial_capacity);
}

#[test]
fn callbacks_insert_in_admission_order_not_completion_order() {
    let store = NetworkActivityStore::new(7);
    let first = store.admit(announce(10)).unwrap();
    let second = store.admit(announce(10)).unwrap();
    let third = store.admit(announce(10)).unwrap();
    store.record(third);
    store.record(first);
    store.record(second);
    assert_eq!(
        store
            .project(20)
            .rows
            .iter()
            .map(|row| row.record_id)
            .collect::<Vec<_>>(),
        vec![3, 2, 1]
    );
}

#[test]
fn clear_rejects_delayed_old_callbacks_but_preserves_later_same_tick_callbacks() {
    let store = NetworkActivityStore::new(7);
    record(&store, 10);
    let delayed = store.admit(announce(10)).unwrap();
    let cleared_revision = store.clear();
    store.record(delayed);
    let cleared = store.project(10);
    assert!(cleared.rows.is_empty());
    assert_eq!(cleared.revision, cleared_revision);
    assert_eq!(cleared.evictions, 0);
    record(&store, 10);
    let current = store.project(10);
    assert_eq!(current.rows.len(), 1);
    assert_eq!(current.rows[0].record_id, 3);
    assert_eq!(current.rows[0].age_millis, 0);
    assert!(current.revision > cleared_revision);
}

#[test]
fn clear_resets_only_window_evictions_and_never_reuses_record_ids() {
    let store = NetworkActivityStore::new(7);
    for _ in 0..=ACTIVITY_CAPACITY {
        record(&store, 10);
    }
    assert_eq!(store.project(10).evictions, 1);
    store.clear();
    record(&store, 10);
    let current = store.project(10);
    assert_eq!(current.evictions, 0);
    assert_eq!(current.rows[0].record_id, 202);
}

#[test]
fn activity_composes_the_existing_observer_and_copies_only_bounded_facts() {
    let store = Arc::new(NetworkActivityStore::new(7));
    let forwarded = Arc::new(AtomicU64::new(0));
    let count = Arc::clone(&forwarded);
    let clearing = Arc::clone(&store);
    let mut observer = store.compose_observer(move |observation| {
        assert_eq!(observation, announce(10));
        count.fetch_add(1, Ordering::SeqCst);
        // Existing service callbacks run with no activity lock held. A clear
        // between callback admission and insertion cannot revive this row.
        clearing.clear();
    });
    observer(announce(10));
    assert_eq!(forwarded.load(Ordering::SeqCst), 1);
    assert!(store.project(20).rows.is_empty());

    let mut observer = store.compose_observer(|_| {});
    observer(announce(10));
    assert_eq!(
        store.project(20).rows,
        vec![LocalAnnounceActivity {
            record_id: 2,
            destination: [1; 16],
            announced_identity: [2; 16],
            source_interface: vec![3; 8],
            hops: 2,
            age_millis: 10,
            is_path_response: true,
        }]
    );
}

#[test]
fn exhausted_activity_ids_do_not_wrap_or_prevent_lxmf_observation() {
    let store = Arc::new(NetworkActivityStore::new(7));
    store.admitted.store(u64::MAX, Ordering::SeqCst);
    let forwarded = Arc::new(AtomicU64::new(0));
    let count = Arc::clone(&forwarded);
    let mut observer = store.compose_observer(move |_| {
        count.fetch_add(1, Ordering::SeqCst);
    });
    observer(announce(10));
    assert_eq!(forwarded.load(Ordering::SeqCst), 1);
    assert!(store.project(20).rows.is_empty());
    assert_eq!(store.admitted.load(Ordering::SeqCst), u64::MAX);
}

#[test]
fn restored_clock_projects_route_ages_and_expiry_without_using_uptime_or_wall_time() {
    let origin = (1_u64 << 53) + 100;
    let clock = personal_rns::manifold::tokio::TokioClock::start_at(
        personal_rns::units::InstantMillis(origin),
    );
    let now = clock.now().0;
    let route = prns_host::RouteSnapshot {
        destination: prns_host::DestinationHash::new([1; 16]),
        hops: 2,
        via_identity: Some(prns_host::IdentityHash::new([2; 16])),
        interface_id: prns_host::InterfaceId::new([3; 8]),
        learned_at_millis: now - 90,
        last_route_activity_at_millis: now - 30,
        expires_at_millis: now + 500,
    };
    let projected = project_routes(std::slice::from_ref(&route), now);
    assert_eq!(
        projected[0],
        LocalNetworkRouteSnapshot {
            destination: [1; 16],
            via_identity: Some([2; 16]),
            interface_id: vec![3; 8],
            hops: 2,
            learned_age_millis: 90,
            last_activity_age_millis: 30,
            expires_in_millis: 500,
            expired: false,
        }
    );
    let expired = project_routes(std::slice::from_ref(&route), now + 500);
    assert!(expired[0].expired);
    assert_eq!(expired[0].expires_in_millis, 0);
    let future = project_routes(&[route], 0);
    assert_eq!(future[0].learned_age_millis, 0);
    assert_eq!(future[0].last_activity_age_millis, 0);
    let store = NetworkActivityStore::new(7);
    record(&store, now - 10);
    assert_eq!(store.project(now).rows[0].age_millis, 10);
    assert_eq!(store.project(0).rows[0].age_millis, 0);
}
