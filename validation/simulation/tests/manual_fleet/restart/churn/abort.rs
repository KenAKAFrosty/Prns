use super::*;

#[test]
fn dropping_a_full_fleet_with_pending_requests_allows_reboot_on_the_same_clock() {
    let medium = fixture::medium();
    let mut driver = ManualTimeDriver::new(
        ManualMedium::Frames(medium.clone()),
        Duration::from_millis(1),
    )
    .unwrap_or_else(|error| unreachable!("abort/rebuild driver: {error}"));
    let mut runner = ManualTaskRunner::new(&mut driver, nonzero(ACTOR_CAPACITY));
    let old: Vec<_> = (0..NODES)
        .map(|index| start(&mut runner, &medium, index))
        .collect();
    for pair in 0..PAIRS {
        connect(&medium, &old, pair);
    }
    let previous_links = links(&mut runner, &old, 0..PAIRS);
    echo_round(&mut runner, &old, &previous_links, 0, 0..PAIRS);
    for pair in 0..PAIRS {
        assert_eq!(
            medium.set_reachability(
                old[pair * 2].endpoint,
                old[pair * 2 + 1].endpoint,
                Reachability::Isolated,
            ),
            Ok(TopologyMutation::Applied)
        );
    }
    let abandoned = lost_requests(&mut runner, &old, &previous_links);
    assert_eq!(abandoned.len(), PAIRS);
    assert!(settle(&mut runner).is_empty());
    assert_eq!(runner.task_count(), NODES + PAIRS);
    let before = runner
        .snapshot()
        .unwrap_or_else(|error| unreachable!("before abort: {error}"));
    drop(runner);
    assert_eq!(driver.snapshot().ok(), Some(before));
    evidence::trace(&medium, NODES);

    // Reach the abandoned request deadlines with no runner or actors left.
    // Replacement boot must neither inherit those timers nor rewind the clock.
    assert!(driver.advance_to_next_event(tick(TIMEOUT_MS)).is_ok());
    let mut runner = ManualTaskRunner::new(&mut driver, nonzero(ACTOR_CAPACITY));
    assert_eq!(now(&runner), TIMEOUT_MS);
    assert!(settle(&mut runner).is_empty());
    let fresh: Vec<_> = (0..NODES)
        .map(|index| start(&mut runner, &medium, index))
        .collect();
    for (old, fresh) in old.iter().zip(&fresh) {
        assert_ne!(fresh.endpoint, old.endpoint);
        assert_eq!(fresh.interface, old.interface);
        assert!(fresh.heard.borrow().is_empty());
        assert_eq!(fresh.born_at, TIMEOUT_MS);
    }
    // Actor IDs are runner-scoped; unlike medium endpoint IDs, their ordinals
    // may repeat in this new runner. Old task IDs must not be used here.
    for old in old {
        assert_eq!(old.control.shutdown.send(()), Err(()));
    }
    for pair in 0..PAIRS {
        connect(&medium, &fresh, pair);
    }
    let current = links(&mut runner, &fresh, 0..PAIRS);
    evidence::capacity(&medium, &fresh);
    for (&pair, &link) in &current {
        assert_ne!(link, previous_links[&pair]);
    }
    echo_round(&mut runner, &fresh, &current, 1, 0..PAIRS);
    evidence::clocks(&mut runner, &fresh);
    assert!(runner.advance_to_next_event(tick(2 * TIMEOUT_MS)).is_ok());
    assert!(settle(&mut runner).is_empty());
    echo_round(&mut runner, &fresh, &current, 2, 0..PAIRS);
    evidence::clocks(&mut runner, &fresh);
    evidence::shutdown(&mut runner, fresh);
    evidence::trace(&medium, 2 * NODES);
}
