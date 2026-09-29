use super::*;

const DISCOVERY_BUDGET_MS: u64 = 10_000;

pub(super) fn discover(runner: &mut ManualTaskRunner<'_, Completion>, nodes: &[LiveNode]) {
    for (index, node) in nodes[..LEAVES].iter().enumerate() {
        announce(&node.control, hash(index));
    }
    let deadline = now(runner) + DISCOVERY_BUDGET_MS;
    loop {
        assert!(settle(runner).is_empty());
        if nodes[LEAVES..]
            .iter()
            .all(|node| node.heard.borrow().len() == LEAVES)
        {
            break;
        }
        assert!(
            now(runner) < deadline,
            "transport announcements must converge"
        );
        assert!(runner.advance_to_next_event(tick(now(runner) + 1)).is_ok());
    }
    let mut expected: Vec<_> = (0..LEAVES).map(hash).collect();
    expected.sort_by_key(|destination| *destination.as_bytes());
    for node in &nodes[LEAVES..] {
        assert_eq!(*node.heard.borrow(), expected);
    }
    // The new transport can hear the last announcement before its jittered
    // rebroadcast reaches every leaf. Poll route observations, not old history.
    loop {
        let ready = Rc::new(std::cell::Cell::new(true));
        let mut tasks = Vec::new();
        for from in 0..NODE_COUNT {
            let handle = nodes[from].control.handle.clone();
            let observations = ready.clone();
            let expected: Vec<_> = (0..LEAVES)
                .filter(|&to| to != from)
                .map(|to| (hash(to), topology::expected_route(nodes, from, to)))
                .collect();
            let task = runner
                .insert(async move {
                    for (destination, (hops, interface)) in expected {
                        let route = handle.route(destination).await;
                        if route.map(|route| (route.hops, route.interface))
                            != Some((hops, interface))
                        {
                            observations.set(false);
                        }
                    }
                    Completion::RoutesChecked { node: from }
                })
                .unwrap_or_else(|error| unreachable!("route check actor: {error}"));
            tasks.push((task, Completion::RoutesChecked { node: from }));
        }
        assert_eq!(
            settle(runner).into_iter().collect::<BTreeMap<_, _>>(),
            tasks.into_iter().collect()
        );
        if ready.get() {
            break;
        }
        assert!(
            now(runner) < deadline,
            "complete route inventory must converge"
        );
        assert!(runner.advance_to_next_event(tick(now(runner) + 1)).is_ok());
    }
}

pub(super) fn establish(
    runner: &mut ManualTaskRunner<'_, Completion>,
    nodes: &[LiveNode],
    from: usize,
    to: usize,
) -> LinkId {
    let handle = nodes[from].control.handle.clone();
    let task = runner
        .insert(async move {
            let link = handle
                .establish_link(hash(to))
                .await
                .unwrap_or_else(|error| unreachable!("routed link: {error:?}"));
            Completion::Linked { node: from, link }
        })
        .unwrap_or_else(|error| unreachable!("link admission: {error}"));
    let results = settle(runner);
    let [(observed, Completion::Linked { node, link })] = results.as_slice() else {
        unreachable!("one routed link: {results:?}")
    };
    assert_eq!((*observed, *node), (task, from));
    *link
}

pub(super) fn echo(
    runner: &mut ManualTaskRunner<'_, Completion>,
    nodes: &[LiveNode],
    from: usize,
    link: LinkId,
    bytes: &[u8],
) {
    let task = request(
        runner,
        from,
        nodes[from].control.handle.clone(),
        link,
        bytes.to_vec(),
    );
    assert_eq!(
        settle(runner),
        [(
            task,
            Completion::Response {
                node: from,
                bytes: bytes.to_vec()
            }
        )]
    );
}

pub(super) fn timeouts(
    runner: &mut ManualTaskRunner<'_, Completion>,
    nodes: &[LiveNode],
    crossing: &[(usize, LinkId)],
) -> BTreeMap<ManualTaskId, Completion> {
    crossing
        .iter()
        .map(|&(from, link)| {
            let handle = nodes[from].control.handle.clone();
            let task = runner
                .insert(async move {
                    assert_eq!(
                        handle
                            .request_with_response_timeout(
                                link,
                                RequestPathHash::of(QUERY_PATH),
                                b"lost transport mapping",
                                RequestResponseTimeout::Exact(DurationMillis(REQUEST_TIMEOUT_MS))
                            )
                            .await,
                        Err(SendError::Failed(SendRequestFailure::Timeout))
                    );
                    Completion::TimedOut { node: from }
                })
                .unwrap_or_else(|error| unreachable!("timeout actor: {error}"));
            (task, Completion::TimedOut { node: from })
        })
        .collect()
}

pub(super) fn expire(
    runner: &mut ManualTaskRunner<'_, Completion>,
    started: u64,
    expected: BTreeMap<ManualTaskId, Completion>,
) {
    for elapsed in 1..REQUEST_TIMEOUT_MS {
        assert!(runner
            .advance_to_next_event(tick(started + elapsed))
            .is_ok());
        assert!(
            settle(runner).is_empty(),
            "routed request cannot expire early"
        );
    }
    assert!(runner
        .advance_to_next_event(tick(started + REQUEST_TIMEOUT_MS))
        .is_ok());
    assert_eq!(
        settle(runner).into_iter().collect::<BTreeMap<_, _>>(),
        expected
    );
    assert_eq!(now(runner), started + REQUEST_TIMEOUT_MS);
}
