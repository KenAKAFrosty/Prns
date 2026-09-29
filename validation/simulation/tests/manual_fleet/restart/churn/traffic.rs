use super::*;

const PAYLOAD_BYTES: usize = 96;

fn payload(pair: usize, round: usize) -> Vec<u8> {
    let mut bytes = vec![0xA5; PAYLOAD_BYTES];
    bytes[..8].copy_from_slice(&(pair as u64).to_be_bytes());
    bytes[8..16].copy_from_slice(&(round as u64).to_be_bytes());
    bytes
}

pub(super) fn links(
    runner: &mut ManualTaskRunner<'_, Completion>,
    nodes: &[LiveNode],
    pairs: impl Iterator<Item = usize>,
) -> BTreeMap<usize, LinkId> {
    let pairs: Vec<_> = pairs.collect();
    for &pair in &pairs {
        announce(&nodes[pair * 2 + 1].control, hash(pair * 2 + 1));
    }
    assert!(settle(runner).is_empty());
    for &pair in &pairs {
        assert_eq!(*nodes[pair * 2].heard.borrow(), [hash(pair * 2 + 1)]);
    }
    let tasks: BTreeMap<_, _> = pairs
        .into_iter()
        .map(|pair| {
            let handle = nodes[pair * 2].control.handle.clone();
            let task = runner
                .insert(async move {
                    let link = handle
                        .establish_link(hash(pair * 2 + 1))
                        .await
                        .unwrap_or_else(|error| unreachable!("pair {pair} link: {error:?}"));
                    Completion::Linked {
                        node: pair * 2,
                        link,
                    }
                })
                .unwrap_or_else(|error| unreachable!("link actor: {error}"));
            (task, pair)
        })
        .collect();
    let completed: BTreeMap<_, _> = settle(runner).into_iter().collect();
    assert_eq!(
        completed.keys().collect::<Vec<_>>(),
        tasks.keys().collect::<Vec<_>>()
    );
    completed
        .into_iter()
        .map(|(task, output)| {
            let pair = tasks[&task];
            let Completion::Linked { node, link } = output else {
                unreachable!("only link results")
            };
            assert_eq!(node, pair * 2);
            (pair, link)
        })
        .collect()
}

pub(super) fn echo_round(
    runner: &mut ManualTaskRunner<'_, Completion>,
    nodes: &[LiveNode],
    current: &BTreeMap<usize, LinkId>,
    round: usize,
    pairs: impl Iterator<Item = usize>,
) {
    let expected: BTreeMap<_, _> = pairs
        .map(|pair| {
            let node = pair * 2;
            let bytes = payload(pair, round);
            let task = request(
                runner,
                node,
                nodes[node].control.handle.clone(),
                current[&pair],
                bytes.clone(),
            );
            (task, Completion::Response { node, bytes })
        })
        .collect();
    assert_eq!(
        settle(runner).into_iter().collect::<BTreeMap<_, _>>(),
        expected
    );
}

pub(super) fn lost_requests(
    runner: &mut ManualTaskRunner<'_, Completion>,
    nodes: &[LiveNode],
    obsolete: &BTreeMap<usize, LinkId>,
) -> BTreeMap<ManualTaskId, Completion> {
    obsolete
        .iter()
        .map(|(&pair, &link)| {
            let node = pair * 2;
            let handle = nodes[node].control.handle.clone();
            let task = runner
                .insert(async move {
                    assert_eq!(
                        handle
                            .request_with_response_timeout(
                                link,
                                RequestPathHash::of(QUERY_PATH),
                                b"old incarnation",
                                RequestResponseTimeout::Exact(DurationMillis(TIMEOUT_MS)),
                            )
                            .await,
                        Err(SendError::Failed(SendRequestFailure::Timeout))
                    );
                    Completion::TimedOut { node }
                })
                .unwrap_or_else(|error| unreachable!("timeout actor: {error}"));
            (task, Completion::TimedOut { node })
        })
        .collect()
}

pub(super) fn expire(
    runner: &mut ManualTaskRunner<'_, Completion>,
    started: u64,
    expected: BTreeMap<ManualTaskId, Completion>,
) {
    for elapsed in 1..TIMEOUT_MS {
        assert!(runner
            .advance_to_next_event(tick(started + elapsed))
            .is_ok());
        assert!(
            settle(runner).is_empty(),
            "no request may expire before its exact deadline"
        );
    }
    assert!(runner
        .advance_to_next_event(tick(started + TIMEOUT_MS))
        .is_ok());
    assert_eq!(
        settle(runner).into_iter().collect::<BTreeMap<_, _>>(),
        expected
    );
    assert_eq!(now(runner), started + TIMEOUT_MS);
}
